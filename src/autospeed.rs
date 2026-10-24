//! `cycles=auto`: the speed a program needs, found from what it does.
//!
//! A game shows how much speed it can use in the frames it draws. While
//! its frame rate follows the speed, it wants more; once the frame rate
//! stays where it is, as the game caps it itself (Stunts at 20 frames a
//! second, Heretic at 35) or waits for the display, more speed only keeps
//! a host core busy; and frames drawn faster than the display shows them
//! are never seen, and some games (Descent) play badly at such rates. So
//! the speed goes up in steps while the frame rate follows, back to where
//! it stopped following, and never past the display's refresh rate, or
//! the frame rate the user asks for instead (`auto_fps`).
//!
//! The frames are counted on the bus (`Activity`): page flips, or where a
//! program draws out of sight and copies the finished picture to video
//! memory, the bursts of writes that copy it. A protected-mode program may
//! copy only what changed since the frame before (Alien Legacy): a burst
//! too small to be a frame that reaches across the screen is one too. A
//! program that checks for keystrokes that aren't there, calls DOS's idle
//! interrupts or polls for the retrace in a loop is waiting, and has speed
//! to spare.
//!
//! Where there are no frames to go by, a real-mode program runs at the
//! real-mode speed (3000, about a 286, DOSBox's for `cycles=auto`): the
//! speed-sensitive games of the 1980s draw without full frames. A
//! protected-mode program gets all the host has while it reads and writes
//! (loading, compiling, Windows), and otherwise goes down to a 486's
//! speed, or a quarter of a faster CPU model's clock: it spins in a loop,
//! as game menus do. (It was written for a 386 or better, and paces
//! itself.) Under an operating system that halts the processor when it has
//! nothing to run (Windows, OS/2, Linux), a program that keeps it from
//! halting is at work and gets all the host has as well. Once a program
//! stops reading and writing, it goes back to the speed it had at once,
//! not a measurement later: programs time their delay loops right after
//! loading, and a loop timed at all the host has waits for ever (Sam &
//! Max's General MIDI driver counts to a million in a timer tick, and
//! hangs if it gets there first).

use crate::bus::Bus;
use crate::video::VideoMode;

/// Emulated time a measurement takes at least, and at most while it waits
/// for enough frames to go by: a frame more or less is a twentieth.
const WINDOW_NS: u64 = 500_000_000;
const LONG_WINDOW_NS: u64 = 2_000_000_000;
const ENOUGH_FRAMES: u64 = 20;
/// Emulated time after a change of speed before a measurement starts.
/// A game that paces its frames by the clock catches up on the frames the
/// slower speed owed it for a second or more, so after a change the
/// frame rate counts once two measurements in a row agree to within
/// `AGREE`, or after `TRIES` of them.
const SETTLE_NS: u64 = 250_000_000;
/// After a step down, which only saves the host's time and can wait, the
/// program settles longer: some adapt to a slower machine for seconds
/// (F-117A draws fewer frames than it can for four), and a step that
/// seems to lose frames goes back up.
const SETTLE_DOWN_NS: u64 = 5_000_000_000;
const AGREE: f64 = 0.15;
const TRIES: u32 = 4;

/// How much more a step up asks for at most, the first time and when the
/// speed it holds at is tried again.
const FIRST_STEP: f64 = 4.0;
const FINE_STEP: f64 = 1.15;
/// A step up whose frame rate grew by at least this share of the speed's
/// growth (in the logarithm) found the program still wanting more: frame
/// rates grow in jumps where a game paces its frames by timer ticks. A
/// step down that kept this share of the frames lost nothing.
const FOLLOWS: f64 = 0.5;
const KEPT: f64 = 0.9;
/// Kept above the speed the frame rate stopped following at.
const HEADROOM: f64 = 1.1;
/// A frame rate this close to the display's refresh rate is all it shows.
const AT_REFRESH: f64 = 0.95;
/// Measurements the speed holds before trying a little more or less, in
/// turn, and the frame rate falling below this share of what it held at,
/// which tries more at once (a scene that takes more work).
const HOLD_WINDOWS: u32 = 30;
const DROPPED: f64 = 0.8;
/// A program that spends this share of its time polling is waiting, and
/// its speed goes down by `WAITING_STEP` a measurement.
const WAITING: f64 = 0.3;
const WAITING_STEP: f64 = 0.8;
/// Keystroke checks and idle calls a second above which a program is
/// waiting: games check once a frame, programs waiting for a key in a loop
/// thousands of times.
const POLLS_PER_SECOND: f64 = 300.0;
/// The least speed of a protected-mode program, which was written for a
/// 386 or better and paces itself: about a 486 at 25 MHz. Less, a game's
/// menus take seconds to draw.
const PROTECTED_LEAST: u32 = 20_000;
/// A protected-mode program halted this share of a measurement: an
/// operating system's idle loop, which halts while nothing runs.
const IDLES: f64 = 0.5;
/// Under such a system, a program that leaves it halted less than this
/// share of a measurement keeps it busy (the timer and disk interrupts
/// still halt it a little).
const BUSY: f64 = 0.05;
/// Bytes a second written to video memory and moved on drives above which
/// a program without frames is at work.
const WORKING: f64 = 32768.0;
/// Emulated time without reads and writes after which a program that got
/// all the host has for them goes back to the speed it had: less than the
/// four timer ticks a delay loop's timing waits for.
const WORK_DONE_NS: u64 = 100_000_000;
/// What a read of the input status register (3DAh) in a retrace-polling
/// loop takes, the I/O's time and the loop's.
const STATUS_READ_NS: f64 = 1200.0;

/// Instructions with no write to video memory that end a burst of them:
/// more than an interrupt handler that runs in the middle of a copy takes.
/// Counted in instructions, not time, so that a program draws the same
/// bursts at any speed.
pub(crate) const BURST_GAP: u64 = 5000;
/// Changes of the display start this close together are one flip (the
/// program writes the start address a byte at a time).
const FLIP_GAP_INSTRUCTIONS: u64 = 1000;

/// What the running program does that tells the speed it needs, counted
/// as it runs.
#[derive(Clone, Debug, Default)]
pub struct Activity {
    /// Bursts of writes to video memory big enough to be a frame copied
    /// there, and page flips: changes of the display start, and the 3dfx
    /// card's buffer swaps.
    pub bursts: u64,
    pub flips: u64,
    /// Bursts too small to be a frame that reached across at least half
    /// the screen: frames copied as the parts of them that changed.
    /// `AutoSpeed::update` counts them for protected-mode programs only.
    pub copies: u64,
    /// Checks for a keystroke that found none, and calls of DOS's and
    /// Windows' idle interrupts.
    pub polls: u64,
    /// Reads of the input status register (3DAh, 3BAh).
    pub status_reads: u64,
    /// Bytes written to video memory, and moved on drives.
    pub video_bytes: u64,
    pub io_bytes: u64,
    /// The burst going on: its bytes, and the instruction of its last write.
    burst: u64,
    last_write: u64,
    /// The lowest and highest place in video memory it wrote, past its
    /// last byte. (The recompiler's own writes to the VGA's planes, which
    /// go on a burst without coming here, leave them out.)
    low: u64,
    high: u64,
    /// A burst's bytes to count as a frame, and its reach to count as a
    /// copy.
    burst_min: u64,
    span_min: u64,
    /// The display start, and the instruction it last changed at.
    start: u32,
    last_flip: u64,
}

/// Where translated code finds `video_write`'s counts (`Activity`'s
/// `video_bytes`, `burst` and `last_write`).
pub(crate) const VIDEO_BYTES_AT: usize = std::mem::offset_of!(Activity, video_bytes);
pub(crate) const BURST_AT: usize = std::mem::offset_of!(Activity, burst);
pub(crate) const LAST_WRITE_AT: usize = std::mem::offset_of!(Activity, last_write);

impl Activity {
    pub fn new() -> Self {
        Self { burst_min: u64::MAX, span_min: u64::MAX, low: u64::MAX, ..Self::default() }
    }

    /// The program wrote `bytes` bytes to video memory at `offset`, at
    /// instruction `at`. (The recompiler counts a block's instructions as
    /// it enters it and gives back those it didn't run, so `at` may go back
    /// a little.)
    #[inline]
    pub fn video_write(&mut self, at: u64, offset: u64, bytes: u64) {
        self.video_bytes += bytes;
        if at.saturating_sub(self.last_write) > BURST_GAP {
            if self.burst >= self.burst_min {
                self.bursts += 1;
            } else if self.high.saturating_sub(self.low) >= self.span_min {
                self.copies += 1;
            }
            self.burst = 0;
            (self.low, self.high) = (u64::MAX, 0);
        }
        self.burst += bytes;
        self.low = self.low.min(offset);
        self.high = self.high.max(offset + bytes);
        self.last_write = at;
    }

    /// The display start is `start` at instruction `at`.
    pub fn display_start(&mut self, at: u64, start: u32) {
        if start != self.start {
            if at.saturating_sub(self.last_flip) > FLIP_GAP_INSTRUCTIONS {
                self.flips += 1;
            }
            self.start = start;
            self.last_flip = at;
        }
    }

    /// The 3dfx card swapped its buffers `swaps` times.
    pub fn swapped(&mut self, swaps: u64) {
        self.flips += swaps;
    }

    pub fn poll(&mut self) {
        self.polls += 1;
    }

    /// The bytes a burst needs to count as a frame: a quarter of what the
    /// program writes for a screen (`screen_bytes`); and as a copy, how far
    /// it reaches: half of that.
    pub fn set_screen(&mut self, bytes: u64) {
        self.burst_min = (bytes / 4).max(500);
        self.span_min = bytes / 2;
    }
}

/// The bytes a program writes for a whole screen in the mode `bus` is in:
/// in the 16-colour modes and the unchained 256-colour ones, one plane's.
pub fn screen_bytes(bus: &Bus) -> u64 {
    let (w, h) = bus.display_size();
    let pixels = (w * h) as u64;
    match bus.video_mode {
        VideoMode::Text40x25 | VideoMode::Text40x25Color => 2000,
        VideoMode::Text80x25 | VideoMode::Text80x25Color | VideoMode::Mono80x25 => 4000,
        VideoMode::Cga320x200Color | VideoMode::Cga320x200 | VideoMode::Cga640x200 => 16000,
        VideoMode::Tandy160x200x16 | VideoMode::Tandy320x200x16 | VideoMode::Tandy640x200x4 => pixels / 2,
        VideoMode::Graphics320x200 if bus.vga.sequencer_regs[0x04] & 0x08 != 0 => pixels,
        VideoMode::Graphics320x200 => pixels / 4,
        VideoMode::Vesa => bus.vbe.mode.map_or(pixels, |m| pixels * m.bytes_per_pixel().max(1) as u64),
        _ => pixels / 8,
    }
}

/// Whether `RUST_DOS_AUTO_TRACE` is set: then each measurement and the
/// speed it chose go to stderr.
fn trace() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUST_DOS_AUTO_TRACE").is_some())
}

/// A measurement's start: the counts as they were.
#[derive(Clone, Copy, Debug)]
struct Mark {
    ns: u64,
    icount: u64,
    idle: u64,
    bursts: u64,
    copies: u64,
    flips: u64,
    polls: u64,
    status_reads: u64,
    output: u64,
}

impl Mark {
    fn of(bus: &Bus) -> Self {
        let a = &bus.activity;
        Mark {
            ns: bus.clock.now_ns(),
            icount: bus.clock.icount,
            idle: bus.clock.idle,
            bursts: a.bursts,
            copies: a.copies,
            flips: a.flips,
            polls: a.polls,
            status_reads: a.status_reads,
            output: a.video_bytes + a.io_bytes,
        }
    }
}

/// What a measurement found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Measurement {
    /// The frames the program drew a second, if it drew any to count.
    pub fps: Option<f64>,
    /// The share of the time it polled (waiting in a loop), and sat
    /// halted (waiting for an interrupt).
    pub polling: f64,
    pub halted: f64,
    /// Bytes a second it wrote to video memory and moved on drives: whether
    /// a program without frames does anything.
    pub output: f64,
    /// The frame rate to aim for: the display's refresh rate, or the
    /// user's (`auto_fps`).
    pub refresh: f64,
}

/// Where the search for the speed is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Step {
    /// Nothing measured yet to compare with.
    Start,
    /// The speed went up from `from`, where the program drew `fps`, and
    /// before that from `below` (a speed and its frame rate), which the
    /// program still drew as fast as it could at.
    Raised { from: u32, fps: f64, below: (u32, f64) },
    /// The speed went down from `from`, where the program drew `fps`.
    Lowered { from: u32, fps: f64 },
    /// The speed is found, at which the program drew `fps`; `left`
    /// measurements before trying a little more (`more`) or less. `recent`
    /// is the frame rate since, smoothed: games' frame rates jitter.
    Hold { fps: f64, left: u32, more: bool, recent: f64 },
}

/// Finds the speed for `CpuSpeed::Auto`.
#[derive(Clone, Debug)]
pub struct AutoSpeed {
    /// The speed of real-mode programs without frames to go by, and the
    /// least there is.
    base: u32,
    /// The frame rate to aim for in place of the display's refresh rate
    /// (`auto_fps`).
    target_fps: Option<u32>,
    mark: Option<Mark>,
    /// No measurement starts before this emulated time.
    settle_until: u64,
    /// Since the last change of speed: the frame rates measured that
    /// didn't agree yet, and how many.
    unsettled: Option<(f64, u32)>,
    step: Step,
    /// While a protected-mode program gets all the host has for its reads
    /// and writes, the speed it had before.
    working: Option<u32>,
    /// The bytes it had read and written, and when that last changed.
    output: (u64, u64),
    /// The least speed of a protected-mode program for the CPU model
    /// (`CpuModel::auto_least`), when that is more than `PROTECTED_LEAST`.
    protected_least: u32,
    /// The protected-mode system halted at least `IDLES` of a measurement
    /// since it started: it has an idle loop. Kept until the machine goes
    /// back to real mode (a reboot, the program's end), and in save states
    /// (`Cpu::os_idles`).
    idles: bool,
    /// Whether a program that keeps such a system busy gets all the host
    /// has (`auto_busy_os`).
    busy_os: bool,
}

impl AutoSpeed {
    pub fn new(base: u32, target_fps: Option<u32>) -> Self {
        Self {
            base,
            target_fps,
            mark: None,
            settle_until: 0,
            unsettled: None,
            step: Step::Start,
            working: None,
            output: (0, 0),
            protected_least: PROTECTED_LEAST,
            idles: false,
            busy_os: true,
        }
    }

    /// The same search with another base and target, keeping what it knows
    /// of the machine (the CPU model's least speed, whether the system
    /// idles) and `busy_os`.
    pub fn with(&self, base: u32, target_fps: Option<u32>) -> Self {
        Self {
            protected_least: self.protected_least,
            idles: self.idles,
            busy_os: self.busy_os,
            ..Self::new(base, target_fps)
        }
    }

    /// Whether the protected-mode system was seen idling.
    pub fn idles(&self) -> bool {
        self.idles
    }

    /// The system was seen idling before (a save state's, `Cpu::os_idles`),
    /// or not.
    pub fn set_idles(&mut self, idles: bool) {
        self.idles = idles;
    }

    /// Give a program that keeps an idling system busy all the host has
    /// (`auto_busy_os`), or treat it as any other.
    pub fn set_busy_os(&mut self, on: bool) {
        self.busy_os = on;
    }

    /// The least speed of a protected-mode program on this CPU model.
    pub fn set_protected_least(&mut self, least: u32) {
        self.protected_least = least.max(PROTECTED_LEAST);
    }

    /// The machine left protected mode: whatever runs next hasn't shown
    /// an idle loop yet.
    pub fn left_protected_mode(&mut self) {
        self.idles = false;
        self.reset();
    }

    pub fn base(&self) -> u32 {
        self.base
    }

    pub fn target_fps(&self) -> Option<u32> {
        self.target_fps
    }

    /// Start over, as for another program, or from a machine that jumped
    /// (a save state loaded).
    pub fn reset(&mut self) {
        self.mark = None;
        self.settle_until = 0;
        self.unsettled = None;
        self.step = Step::Start;
        self.working = None;
    }

    /// After a frame: once a measurement is complete, the speed it asks
    /// for, if that isn't the one `bus` runs at. `protected` says whether
    /// the program switched to protected mode, and `host_max` is as fast
    /// as the host keeps up with.
    pub fn update(&mut self, bus: &Bus, protected: bool, host_max: u32) -> Option<u32> {
        let now = Mark::of(bus);
        if now.output != self.output.0 {
            self.output = (now.output, now.ns);
        }
        if let Some(before) = self.working
            && now.ns.saturating_sub(self.output.1) >= WORK_DONE_NS
        {
            if trace() {
                eprintln!("[auto] work done: back to {}", before);
            }
            self.reset();
            self.settle_until = now.ns + SETTLE_NS;
            return (before != bus.clock.cycles_per_ms()).then_some(before);
        }
        let Some(mark) = self.mark else {
            if now.ns >= self.settle_until {
                self.mark = Some(now);
            }
            return None;
        };
        let ns = now.ns.saturating_sub(mark.ns);
        let (bursts, flips) = (now.bursts - mark.bursts, now.flips - mark.flips);
        // Copies only of protected-mode programs: the speed-sensitive
        // real-mode games of the 1980s draw parts of the screen as they go,
        // and keep the real-mode speed.
        let copies = if protected { now.copies - mark.copies } else { 0 };
        let frames = if flips > 0 { flips } else { bursts + copies };
        if ns < WINDOW_NS || (frames > 0 && frames < ENOUGH_FRAMES && ns < LONG_WINDOW_NS) {
            return None;
        }
        self.mark = Some(now);
        let seconds = ns as f64 / 1e9;
        let ran = now.icount.saturating_sub(mark.icount).max(1) as f64;
        let polls = (now.polls - mark.polls) as f64 / seconds;
        let status = (now.status_reads - mark.status_reads) as f64 * STATUS_READ_NS / ns as f64;
        let refresh = match (self.target_fps, &bus.voodoo) {
            (Some(fps), _) => fps as f64,
            (None, Some(v)) if v.output() => v.refresh_hz(),
            (None, _) => bus.vga.peek_timing().hz(),
        };
        let mut fps = (frames > 0).then(|| frames as f64 / seconds);
        if let (Some(rate), Some((before, tries))) = (fps, self.unsettled) {
            let settled = (rate / before - 1.0).abs() <= AGREE || tries + 1 >= TRIES;
            if !settled {
                self.unsettled = Some((rate, tries + 1));
                return None;
            }
            fps = Some((rate + before) / 2.0);
        }
        self.unsettled = None;
        let measured = Measurement {
            fps,
            polling: if polls > POLLS_PER_SECOND { status.max(0.5) } else { status },
            halted: now.idle.saturating_sub(mark.idle) as f64 / ran,
            output: (now.output - mark.output) as f64 / seconds,
            refresh,
        };
        let current = bus.clock.cycles_per_ms();
        let working = self.working.take();
        let next = self.decide(current, &measured, protected, host_max);
        if self.working.is_some() {
            self.working = working.or(self.working);
        }
        if trace() {
            eprintln!(
                "[auto] {} -> {} (host {}): {:?} {:?} idles={}",
                current, next, host_max, measured, self.step, self.idles
            );
        }
        if next == current {
            return None;
        }
        self.mark = None;
        let lowered = matches!(self.step, Step::Lowered { .. });
        self.settle_until = now.ns + if lowered { SETTLE_DOWN_NS } else { SETTLE_NS };
        self.unsettled = Some((f64::NAN, 0));
        Some(next)
    }

    /// The speed to go on at from `current`, after a measurement there.
    ///
    /// Below the speed where it stops, a program's frame rate follows the
    /// speed: from the frame rates at two speeds, one below that point, it
    /// is at the speed the lower one would have grown to the higher frame
    /// rate at.
    pub fn decide(&mut self, current: u32, m: &Measurement, protected: bool, host_max: u32) -> u32 {
        // A faster model's least speed only up to what the host has.
        let base = if protected {
            self.base.max(PROTECTED_LEAST).max(self.protected_least.min(host_max))
        } else {
            self.base
        };
        let top = host_max.max(base);
        let clamp = |speed: f64| (speed.round() as u32).clamp(base, top);
        let c = current as f64;
        if protected && m.halted >= IDLES {
            self.idles = true;
        }
        if m.polling >= WAITING {
            self.step = Step::Start;
            return clamp(c * WAITING_STEP);
        }
        let Some(fps) = m.fps else {
            self.step = Step::Start;
            // A protected-mode program at work without frames loads, or
            // compiles, or is Windows: it reads and writes. Under a system
            // that idles, one that keeps the processor from halting works
            // too (3DMark 2000 builds a scene for seconds, with disk reads
            // the emulator's own services don't see). Otherwise one that
            // does neither spins in a loop, as a game's menu does.
            return if !protected {
                base
            } else if m.halted >= IDLES {
                clamp(c)
            } else if m.output >= WORKING {
                self.working = Some(clamp(c));
                top
            } else if self.busy_os && self.idles && m.halted < BUSY {
                top
            } else {
                clamp(c * WAITING_STEP)
            };
        };
        // Frames nobody sees: as many less as the frame rate is too high
        // (it follows the speed there).
        if fps > m.refresh / AT_REFRESH {
            self.step = Step::Hold { fps: m.refresh, left: HOLD_WINDOWS, more: false, recent: m.refresh };
            return clamp(c * m.refresh / fps);
        }
        // Hold at `speed`; the frame rate to hold is measured there, unless
        // that is here.
        let hold = |this: &mut Self, speed: f64, left: u32, more: bool| {
            let speed = clamp(speed);
            let fps = if speed == current { fps } else { f64::NAN };
            this.step = Step::Hold { fps, left, more, recent: fps };
            speed
        };
        // More, by as much as would take the frame rate to the display's.
        let up = |this: &mut Self, most: f64, below: (u32, f64)| {
            if fps >= m.refresh * AT_REFRESH || current >= top {
                return hold(this, c, HOLD_WINDOWS, false);
            }
            this.step = Step::Raised { from: current, fps, below };
            clamp(c * (m.refresh / fps).clamp(FINE_STEP, most))
        };
        let down = |this: &mut Self| {
            if current <= base {
                return hold(this, c, HOLD_WINDOWS, true);
            }
            this.step = Step::Lowered { from: current, fps };
            clamp(c / FINE_STEP)
        };
        let here = (current, fps);
        match self.step {
            Step::Start => up(self, FIRST_STEP, here),
            Step::Raised { from, fps: before, below } if current > from => {
                if fps / before >= (c / from as f64).powf(FOLLOWS) {
                    up(self, FIRST_STEP, (from, before))
                } else if c <= from as f64 * FINE_STEP * 1.01 {
                    // A small step that gained nothing: where it came
                    // from was enough.
                    hold(self, from as f64, HOLD_WINDOWS, false)
                } else {
                    // A big step may have gone past the point already:
                    // reckon from the speed before it, and check the
                    // reckoning with a small step up at once (frame rates
                    // that grow in jumps make it too low).
                    let (low, low_fps) = (below.0 as f64, below.1);
                    let knee = (low * fps / low_fps).clamp(low, c);
                    hold(self, knee * HEADROOM, 1, true)
                }
            }
            Step::Lowered { from, fps: before } if current < from => {
                if fps >= before * KEPT {
                    down(self)
                } else {
                    let knee = (c * before / fps).clamp(c, from as f64);
                    hold(self, (knee * HEADROOM).min(from as f64 * HEADROOM), HOLD_WINDOWS, true)
                }
            }
            Step::Hold { fps: held, left, more, recent } => {
                let held = if held.is_nan() { fps } else { held };
                let recent = if recent.is_nan() { fps } else { (recent + fps) / 2.0 };
                if recent < held * DROPPED {
                    up(self, FIRST_STEP, here)
                } else if left > 1 {
                    self.step = Step::Hold { fps: held, left: left - 1, more, recent };
                    current
                } else if more {
                    up(self, FINE_STEP, here)
                } else {
                    down(self)
                }
            }
            // The speed changed from elsewhere (the host's limit): measure
            // from here.
            Step::Raised { .. } | Step::Lowered { .. } => up(self, FIRST_STEP, here),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: u32 = 800_000;

    /// A game that draws `per_cycle` frames a second for each instruction
    /// per ms, up to `cap` frames a second.
    fn game(per_cycle: f64, cap: f64) -> impl Fn(u32) -> Measurement {
        move |speed| Measurement {
            fps: Some((speed as f64 * per_cycle).min(cap)),
            polling: 0.0,
            halted: 0.0,
            output: 0.0,
            refresh: 70.0,
        }
    }

    /// The speeds `auto` runs `program` at, one a measurement.
    fn run(program: impl Fn(u32) -> Measurement, protected: bool, measurements: usize) -> Vec<u32> {
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 3000;
        (0..measurements)
            .map(|_| {
                speed = auto.decide(speed, &program(speed), protected, HOST);
                speed
            })
            .collect()
    }

    #[test]
    fn a_game_that_draws_ever_faster_stops_at_the_refresh_rate() {
        // Descent: 0.8 frames a second for each 1000 instructions per ms.
        let speeds = run(game(0.0008, f64::INFINITY), true, 40);
        let settled = speeds[10];
        assert!((70_000..=95_000).contains(&settled), "{:?}", speeds);
        assert!(speeds[10..].iter().all(|&s| s.abs_diff(settled) < settled / 5), "{:?}", speeds);
    }

    #[test]
    fn a_game_that_caps_its_frames_stops_where_they_stop_following() {
        // Stunts: 20 frames a second from about 11000 on.
        let speeds = run(game(20.0 / 11_000.0, 20.0), false, 40);
        let settled = speeds[12];
        assert!((11_000..=16_000).contains(&settled), "{:?}", speeds);
        // Heretic: 35 from about 60000 on.
        let speeds = run(game(35.0 / 60_000.0, 35.0), true, 40);
        assert!((60_000..=85_000).contains(&speeds[12]), "{:?}", speeds);
        // A while later it tries less, finds that loses frames, and goes
        // back.
        assert!(speeds[12..].iter().all(|&s| s >= 45_000), "{:?}", speeds);
        assert!(speeds[39] >= 60_000, "{:?}", speeds);
    }

    #[test]
    fn a_game_whose_frames_grow_slower_than_the_speed() {
        // F-117A: its frame rate grows in jumps of whole timer ticks, 4 at
        // 3000, 10 at 12000, and stays at 16.5 from 20000 on.
        let f117 = |speed: u32| Measurement {
            fps: Some(match speed {
                0..=3000 => 4.0,
                3001..=12_000 => 4.0 + 6.0 * (speed - 3000) as f64 / 9000.0,
                12_001..=20_000 => 10.0 + 6.5 * (speed - 12_000) as f64 / 8000.0,
                _ => 16.5,
            }),
            polling: 0.0,
            halted: 0.0,
            output: 0.0,
            refresh: 70.0,
        };
        let speeds = run(f117, false, 20);
        assert!((20_000..=30_000).contains(&speeds[19]), "{:?}", speeds);
    }

    #[test]
    fn a_heavier_scene_gets_more_at_once() {
        let mut auto = AutoSpeed::new(3000, None);
        let light = game(35.0 / 30_000.0, 35.0);
        let heavy = game(35.0 / 60_000.0, 35.0);
        let mut speed = 3000;
        for _ in 0..15 {
            speed = auto.decide(speed, &light(speed), false, HOST);
        }
        assert!(speed < 45_000, "{}", speed);
        for _ in 0..8 {
            speed = auto.decide(speed, &heavy(speed), false, HOST);
        }
        assert!(speed >= 60_000, "{}", speed);
    }

    #[test]
    fn no_frames_to_go_by() {
        let nothing = |halted, output| move |_| Measurement { fps: None, polling: 0.0, halted, output, refresh: 70.0 };
        // Real mode: the real-mode speed; protected mode: all the host has
        // while it reads and writes, unless it sits halted.
        assert_eq!(run(nothing(0.0, 1e6), false, 3), [3000, 3000, 3000]);
        assert_eq!(run(nothing(0.0, 1e6), true, 2), [HOST, HOST]);
        assert_eq!(run(nothing(0.9, 1e6), true, 2), [PROTECTED_LEAST, PROTECTED_LEAST]);
        // Spinning without doing anything, it gets less.
        let mut auto = AutoSpeed::new(3000, None);
        assert_eq!(auto.decide(100_000, &nothing(0.0, 100.0)(0), true, HOST), 80_000);
    }

    #[test]
    fn a_program_at_work_keeps_the_speed_to_go_back_to() {
        let mut auto = AutoSpeed::new(3000, None);
        let loading = Measurement { fps: None, polling: 0.0, halted: 0.0, output: 1e6, refresh: 70.0 };
        assert_eq!(auto.decide(PROTECTED_LEAST, &loading, true, HOST), HOST);
        assert_eq!(auto.working, Some(PROTECTED_LEAST), "where it goes back to");
        // Anything else isn't work.
        let spinning = Measurement { output: 0.0, ..loading };
        auto.working = None;
        auto.decide(HOST, &spinning, true, HOST);
        assert_eq!(auto.working, None);
    }

    #[test]
    fn a_waiting_program_gets_less() {
        let mut auto = AutoSpeed::new(3000, None);
        let waiting = Measurement { fps: None, polling: 0.5, halted: 0.0, output: 0.0, refresh: 70.0 };
        let mut speed = 100_000;
        for _ in 0..30 {
            speed = auto.decide(speed, &waiting, false, HOST);
        }
        assert_eq!(speed, 3000);
        // Protected-mode programs no slower than a 486.
        for _ in 0..30 {
            speed = auto.decide(speed, &waiting, true, HOST);
        }
        assert_eq!(speed, PROTECTED_LEAST);
    }

    #[test]
    fn a_target_frame_rate_takes_the_refresh_rates_place() {
        // Descent at `auto_fps=35`, which `update` aims for in place of
        // the display's 70.
        let descent = |speed: u32| Measurement { refresh: 35.0, ..game(0.0008, f64::INFINITY)(speed) };
        let speeds = run(descent, true, 40);
        let settled = speeds[10];
        assert!((35_000..=48_000).contains(&settled), "{:?}", speeds);
        assert!(speeds[10..].iter().all(|&s| s.abs_diff(settled) < settled / 5), "{:?}", speeds);
    }

    #[test]
    fn a_busy_program_under_a_system_that_idles_gets_all_the_host_has() {
        let measure =
            |halted| Measurement { fps: None, polling: 0.0, halted, output: 0.0, refresh: 60.0 };
        // 3DMark 2000 building a scene: no frames, no I/O it can see, never
        // halted. Before the system was seen idling, it is a spinning menu.
        let mut auto = AutoSpeed::new(3000, None);
        assert_eq!(auto.decide(100_000, &measure(0.0), true, HOST), 80_000);
        // Windows' desktop halts most of the time; after that, the same
        // program is at work.
        assert_eq!(auto.decide(80_000, &measure(0.95), true, HOST), 80_000);
        assert_eq!(auto.decide(80_000, &measure(0.0), true, HOST), HOST);
        assert_eq!(auto.decide(HOST, &measure(0.01), true, HOST), HOST);
        // One that halts some of the time isn't busy.
        assert_eq!(auto.decide(100_000, &measure(0.2), true, HOST), 80_000);
        // A new search (another speed, a state loaded) keeps what it knows.
        let mut kept = auto.with(3000, None);
        kept.reset();
        assert_eq!(kept.decide(100_000, &measure(0.0), true, HOST), HOST);
        // Not with `auto_busy_os=false`.
        let mut off = kept.with(3000, None);
        off.set_busy_os(false);
        assert_eq!(off.decide(100_000, &measure(0.0), true, HOST), 80_000);
        // Back in real mode, nothing is known.
        kept.left_protected_mode();
        assert_eq!(kept.decide(100_000, &measure(0.0), true, HOST), 80_000);
        // A real-mode program halting says nothing.
        let mut dos = AutoSpeed::new(3000, None);
        dos.decide(3000, &measure(0.95), false, HOST);
        assert_eq!(dos.decide(100_000, &measure(0.0), true, HOST), 80_000);
    }

    #[test]
    fn a_faster_model_goes_less_far_down() {
        let waiting =
            Measurement { fps: None, polling: 0.5, halted: 0.0, output: 0.0, refresh: 70.0 };
        let settle = |auto: &mut AutoSpeed, protected, host| {
            let mut speed = 200_000;
            for _ in 0..30 {
                speed = auto.decide(speed, &waiting, protected, host);
            }
            speed
        };
        let mut auto = AutoSpeed::new(3000, None);
        auto.set_protected_least(crate::cpu::CpuModel::PentiumII.auto_least());
        assert_eq!(settle(&mut auto, true, HOST), 112_500);
        // Not past what the host has, and not for real-mode programs.
        assert_eq!(settle(&mut auto, true, 60_000), 60_000);
        assert_eq!(settle(&mut auto, false, HOST), 3000);
        // The 486 keeps the 486's.
        auto.set_protected_least(crate::cpu::CpuModel::I486.auto_least());
        assert_eq!(settle(&mut auto, true, HOST), PROTECTED_LEAST);
    }

    /// Alien Legacy's frame copy where little changed: its REPE CMPSD loop
    /// writes the last dword of each of the 200 rows it compares, ten
    /// instructions apart.
    fn copy_changes(bus: &mut Bus) {
        for row in 0..200 {
            bus.clock.icount += 10;
            bus.write_32(0xA0000 + row * 320 + 316, 0x0101_0101);
        }
    }

    #[test]
    fn a_frame_copied_as_what_changed_counts() {
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        crate::interrupts::int10::set_mode(&mut cpu, 0x13);
        let bus = &mut cpu.bus;
        bus.activity.set_screen(screen_bytes(bus));
        for _ in 0..5 {
            bus.clock.icount += 100_000;
            copy_changes(bus);
        }
        // The last one ends with the next write.
        bus.clock.icount += 100_000;
        bus.write_8(0xA0000, 1);
        assert_eq!((bus.activity.bursts, bus.activity.copies), (0, 5));
        // A sprite of the same bytes in one place is no frame.
        for _ in 0..5 {
            bus.clock.icount += 100_000;
            for row in 0..50 {
                bus.clock.icount += 10;
                bus.write_32(0xA0000 + (100 + row) * 320 + 160, 0x0202_0202);
                bus.write_32(0xA0000 + (100 + row) * 320 + 164, 0x0202_0202);
                bus.write_32(0xA0000 + (100 + row) * 320 + 168, 0x0202_0202);
                bus.write_32(0xA0000 + (100 + row) * 320 + 172, 0x0202_0202);
            }
        }
        bus.clock.icount += 100_000;
        bus.write_8(0xA0000, 1);
        assert_eq!((bus.activity.bursts, bus.activity.copies), (0, 5));
    }

    /// The speeds `auto` runs a game at that draws a frame in memory in
    /// `per_frame` instructions, as fast as it can, and copies to the
    /// screen only what changed, for `seconds` of emulated time.
    fn run_copying(protected: bool, per_frame: u64, seconds: u64) -> Vec<u32> {
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        crate::interrupts::int10::set_mode(&mut cpu, 0x13);
        let bus = &mut cpu.bus;
        bus.set_cycles_per_ms(3000);
        let mut auto = AutoSpeed::new(3000, None);
        let mut speeds = Vec::new();
        while bus.clock.now_ns() < seconds * 1_000_000_000 {
            bus.clock.icount += per_frame;
            copy_changes(bus);
            bus.sync_display();
            if let Some(speed) = auto.update(bus, protected, 2_000_000) {
                bus.set_cycles_per_ms(speed);
                speeds.push(speed);
            }
        }
        speeds
    }

    #[test]
    fn a_game_that_copies_what_changed_gets_the_speed_its_frames_need() {
        // 1.2 million instructions a frame: the 70 frames a second the
        // display shows at about 84,000 instructions per ms.
        let speeds = run_copying(true, 1_200_000, 60);
        let settled = *speeds.last().unwrap();
        assert!((70_000..=100_000).contains(&settled), "{:?}", speeds);
        assert!(speeds.iter().all(|&s| s <= 200_000), "{:?}", speeds);
        // A real-mode program's copies don't count: the real-mode speed.
        assert!(run_copying(false, 1_200_000, 10).iter().all(|&s| s == 3000));
    }

    #[test]
    fn never_more_than_the_host_has() {
        let speeds = run(game(0.00001, f64::INFINITY), true, 20);
        assert!(speeds.iter().all(|&s| s <= HOST));
        assert_eq!(*speeds.last().unwrap(), HOST);
    }
}
