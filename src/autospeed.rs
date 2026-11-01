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
//! nothing to run (Windows, OS/2, Linux, and the built-in Windows, which
//! is known to from the start), a program that keeps it from halting is
//! at work and gets all the host has as well. Once a program stops
//! reading and writing, it goes back to the speed it had at once, not a
//! measurement later: programs time their delay loops right after
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
/// rates grow in jumps where a game paces its frames by timer ticks.
const FOLLOWS: f64 = 0.5;
/// Steps down that kept this share of the frames the speed held at lost
/// nothing. Each step is measured against the held frame rate: a game
/// whose frame rate falls a little with each step (Myth's 3Dfx renderer
/// draws 29.8 frames a second at 110,000 and 26 at 44,000, about 2% a
/// step) passes every step measured against the one before it. A step
/// down only saves the host's time, so it may cost no more than the
/// measurements' noise.
const KEPT: f64 = 0.97;
/// Measurements a step down is judged on, that a frame rate needs before it
/// sets the record steps down are held to, and that a hold's frame rate
/// below `KEPT` of the held one is believed after: at one speed Myth
/// measures anything from 26 to 30 frames a second. A frame more or less in
/// a measurement is a twentieth, so Doom's 35 reads anything from 34.3 to
/// 36.4. A record of 36.4 from one measurement would fail steps down that
/// lost nothing; over `JUDGED` the frames at the ends weigh a quarter of
/// that, less than `KEPT` allows.
const JUDGED: u32 = 4;
/// Kept above the speed the frame rate stopped following at.
const HEADROOM: f64 = 1.1;
/// A frame rate this close to the display's refresh rate is all it shows.
const AT_REFRESH: f64 = 0.95;
/// Measurements the speed holds before trying a little more or less, in
/// turn, and the frame rate falling below this share of what it held at,
/// which tries more (a scene that takes more work). It takes two such
/// measurements in a row: one can be a moment's dip, and Doom's screen wipe
/// fills one measurement at 22 frames a second between two at 35. The dip
/// counts for nothing: the speed it would send up takes minutes to come
/// back, one `SETTLE_DOWN_NS` a step.
const HOLD_WINDOWS: u32 = 30;
const DROPPED: f64 = 0.8;
/// A step down that lost frames costs them for all of `SETTLE_DOWN_NS` and
/// the measurements after it, so each one doubles the hold before the next
/// try, up to this many times, until the scene changes. A game at its knee
/// would pay that every half a minute: Doom lost a frame a second to it.
const MOST_BACKOFFS: u32 = 2;
/// Emulated time running slower than this share of the host's didn't keep
/// up: the host can't run that speed now, whatever its estimate says (a
/// battle in Myth at 40,000 instructions per ms ran at 0.25 to 0.6 of real
/// time, the sound running dry, while the estimate stayed above 80,000).
/// It takes a measurement below this share and the pace smoothed over the
/// last ones below it too: a 70 ms hitch of the host drops 7% of a
/// measurement whatever the speed, and taking a step and the cap for each
/// of those would leave the speed at the floor. The speed then goes no
/// higher than a step below it, or what the host ran if it falls behind
/// again. That limit rises by `FINE_STEP` for each `LAGGED_WINDOWS`
/// measurements the host keeps up, so a host that stalls now and then
/// loses a step for a while and no more, and one that can't run a speed
/// meets it again only that often. A scene that takes more work says
/// nothing of a limit a lighter one found (Unreal's 60 frame a second
/// fly-through at 100,000 and the 10 frame a second one after it, which the
/// host runs at 250,000), so a frame rate that falls by `DROPPED` at the
/// limit multiplies it by `HEAVIER`. Too much meets the limit again at once.
const KEPT_UP: f64 = 0.95;
const HEAVIER: f64 = 2.0;
const LAGGED_WINDOWS: u32 = HOLD_WINDOWS;
/// Falling behind once at less than the host estimate over this is a stall.
const STALLED: f64 = 2.0;
/// How many steps of `FINE_STEP` above the record's speed are tried for its
/// frame rate (see `Record`).
const CLIMBS: u32 = 2;
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
    /// A burst's bytes to count as a frame, and its reach and bytes to
    /// count as a copy.
    burst_min: u64,
    span_min: u64,
    copy_min: u64,
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
        Self {
            burst_min: u64::MAX,
            span_min: u64::MAX,
            copy_min: u64::MAX,
            low: u64::MAX,
            ..Self::default()
        }
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
            } else if self.burst >= self.copy_min && self.high.saturating_sub(self.low) >= self.span_min {
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
    /// program writes for a screen (`screen_bytes`). A smaller burst counts
    /// as a copy when it reaches across half of that and writes at least a
    /// 128th of it (500 bytes in mode 13h, more than a mouse pointer and a
    /// clock). Only where the screen's rows follow each other in video
    /// memory (`linear`): in the CGA's, Tandy's and Hercules' graphics they
    /// are interleaved, and text pages lie 4 KB apart.
    pub fn set_screen(&mut self, bytes: u64, linear: bool) {
        self.burst_min = (bytes / 4).max(500);
        (self.span_min, self.copy_min) =
            if linear { (bytes / 2, bytes / 128) } else { (u64::MAX, u64::MAX) };
    }
}

/// Whether the screen's rows lie one after the other in video memory in
/// the mode `bus` is in: the EGA's and VGA's graphics and VESA modes.
pub fn linear_screen(bus: &Bus) -> bool {
    matches!(
        bus.video_mode,
        VideoMode::Ega320x200
            | VideoMode::Ega640x200
            | VideoMode::Ega640x350Mono
            | VideoMode::Ega640x350
            | VideoMode::Vga640x480Mono
            | VideoMode::Vga640x480
            | VideoMode::Graphics320x200
            | VideoMode::Vesa
    )
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
    /// The emulated time dropped behind the wall clock, and the emulated
    /// time that was at. A measurement that isn't settled leaves these for
    /// the next, whose pace covers both.
    lost_ns: u64,
    lost_at: u64,
}

impl Mark {
    fn of(bus: &Bus, lost_ns: u64) -> Self {
        let a = &bus.activity;
        let ns = bus.clock.now_ns();
        Mark {
            lost_ns,
            lost_at: ns,
            ns,
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

/// The frame rate steps down keep `KEPT` of. It moves only by more than
/// `KEPT`, so that measurements' noise doesn't walk it down, as the frame
/// rate itself would. It goes up when the program draws more anywhere.
/// Holding below `at` on fewer frames than `KEPT` of it goes back to `at`,
/// and from there up `CLIMBS` steps. It goes down only if that doesn't
/// bring the frames back: the scene got heavier, or it was a lighter one's
/// or a lucky measurement's. Coming down at a speed that steps down had
/// reached would let them go on from a lower frame rate each time, and
/// that is the slide `KEPT` is there to stop.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Record {
    fps: f64,
    /// The least speed known to draw it: a step down that keeps it takes
    /// this along. From a step past the host's estimate, that estimate: the
    /// speed itself is one the host can't go back to (see `Step::Overshot`).
    at: u32,
    /// The steps taken above the speed it was missed at.
    climbed: u32,
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
    /// Emulated seconds a second of the host's time: 1 kept real time,
    /// less fell behind (and dropped what it didn't catch up on).
    pub pace: f64,
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
    /// The last step up gained nothing: the speed stays while it measures
    /// what the frame rate is with all it could want, for the record.
    /// `below` is as in `Raised`. The frame rates measured here so far add
    /// up to `sum`, `seen` of them. A step that ended above the host's
    /// estimate doesn't stay: Myth's 142,000 reads a host of 129,000 at once
    /// and 90,000 after that, at 0.9 of real time with the sound running dry.
    Overshot {
        below: (u32, f64),
        sum: f64,
        seen: u32,
    },
    /// The speed went down from `from`, where the program drew `fps`. The
    /// frame rates measured here so far add up to `sum`, `seen` of them.
    Lowered {
        from: u32,
        fps: f64,
        sum: f64,
        seen: u32,
    },
    /// The speed is found, at which the program drew `fps`; `left`
    /// measurements before trying a little more (`more`) or less. `recent`
    /// is the frame rate since, smoothed (games' frame rates jitter), over
    /// `seen` measurements. `dipped` says the last measurement fell by
    /// `DROPPED`, which is in neither.
    Hold {
        fps: f64,
        left: u32,
        more: bool,
        recent: f64,
        seen: u32,
        dipped: bool,
    },
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
    /// What steps down keep to. A frame rate falling by a fifth while
    /// holding, and a new search, clear it. Only measurements the host kept
    /// up in set it; falling behind leaves it, for when the host can run
    /// that speed again.
    held: Option<Record>,
    /// Steps down that lost frames since the search started, up to
    /// `MOST_BACKOFFS`.
    backoffs: u32,
    /// Since the host last fell behind (`KEPT_UP`): the most the speed
    /// goes to, and the measurements left before that rises; the pace,
    /// smoothed; and whether the last measurement fell behind.
    lagged: Option<(u32, u32)>,
    pace: f64,
    behind: bool,
    /// While a protected-mode program gets all the host has for its reads
    /// and writes, the speed it had before.
    working: Option<u32>,
    /// The program gets all the host has. The host estimate is its speed
    /// then, so a frame takes it down with the estimate: a measurement at
    /// an estimate that ran high would leave it behind for half a second
    /// (Expendable's sound runs dry). A speed found by the frame rate has
    /// the pace to go by, and waits for that.
    flat_out: bool,
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
            held: None,
            backoffs: 0,
            lagged: None,
            pace: 1.0,
            behind: false,
            working: None,
            flat_out: false,
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
        self.start_over();
        self.lagged = None;
        self.pace = 1.0;
        self.behind = false;
    }

    /// Measure afresh. How the host kept up stays: it says the same of
    /// what the same program does next.
    pub fn start_over(&mut self) {
        self.mark = None;
        self.settle_until = 0;
        self.unsettled = None;
        self.restart_search();
        self.working = None;
        self.flat_out = false;
    }

    /// The search starts again from the next measurement, forgetting the
    /// frame rate it held.
    fn restart_search(&mut self) {
        self.step = Step::Start;
        self.held = None;
        self.backoffs = 0;
    }

    /// After a frame: once a measurement is complete, the speed it asks
    /// for, if that isn't the one `bus` runs at. `protected` says whether
    /// the program switched to protected mode, `host_max` is as fast as
    /// the host keeps up with, as estimated from its frames, and `lost_ns`
    /// the emulated time the host has dropped behind the wall clock so far
    /// (`Pacer::lost`).
    pub fn update(
        &mut self,
        bus: &Bus,
        protected: bool,
        host_max: u32,
        lost_ns: u64,
    ) -> Option<u32> {
        let now = Mark::of(bus, lost_ns);
        if now.output != self.output.0 {
            self.output = (now.output, now.ns);
        }
        if let Some(before) = self.working
            && now.ns.saturating_sub(self.output.1) >= WORK_DONE_NS
        {
            if trace() {
                eprintln!("[auto] work done: back to {}", before);
            }
            // The work may have found the host short of that speed.
            let before = self.lagged.map_or(before, |(most, _)| before.min(most));
            self.start_over();
            self.settle_until = now.ns + SETTLE_NS;
            return (before != bus.clock.cycles_per_ms()).then_some(before);
        }
        let current = bus.clock.cycles_per_ms();
        let cut = (self.flat_out && current > host_max)
            .then(|| host_max.max(self.least(protected, host_max)))
            .filter(|&speed| speed < current);
        let Some(mark) = self.mark else {
            if now.ns >= self.settle_until {
                self.mark = Some(now);
            }
            return cut;
        };
        let ns = now.ns.saturating_sub(mark.ns);
        let (bursts, flips) = (now.bursts - mark.bursts, now.flips - mark.flips);
        // Copies only of protected-mode programs: the speed-sensitive
        // real-mode games of the 1980s draw parts of the screen as they go,
        // and keep the real-mode speed. And only without whole frames: a
        // game that copies its view whole and then draws its gauges at the
        // top and the bottom of the screen draws one frame, not two.
        let copies = if protected { now.copies - mark.copies } else { 0 };
        let frames = match (flips, bursts) {
            (0, 0) => copies,
            (0, bursts) => bursts,
            (flips, _) => flips,
        };
        if ns < WINDOW_NS || (frames > 0 && frames < ENOUGH_FRAMES && ns < LONG_WINDOW_NS) {
            return cut;
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
                // What this measurement dropped counts in the next, over
                // the time of both.
                self.mark = Some(Mark {
                    lost_ns: mark.lost_ns,
                    lost_at: mark.lost_at,
                    ..now
                });
                return cut;
            }
            fps = Some((rate + before) / 2.0);
        }
        self.unsettled = None;
        let paced = now.ns.saturating_sub(mark.lost_at);
        let measured = Measurement {
            fps,
            polling: if polls > POLLS_PER_SECOND { status.max(0.5) } else { status },
            halted: now.idle.saturating_sub(mark.idle) as f64 / ran,
            output: (now.output - mark.output) as f64 / seconds,
            refresh,
            pace: paced as f64 / (paced + now.lost_ns.saturating_sub(mark.lost_ns)) as f64,
        };
        let working = self.working.take();
        let next = self.decide(current, &measured, protected, host_max);
        if self.working.is_some() {
            self.working = working.or(self.working);
        }
        if trace() {
            let longest = bus.host_frames.longest_since(mark.ns);
            let longest = longest.map_or("none".to_string(), |time| {
                format!("{:.1}ms", time.as_secs_f64() * 1e3)
            });
            eprintln!(
                "[auto] {} -> {} (host {}): {:?} {:?} idles={} longest_frame={}",
                current, next, host_max, measured, self.step, self.idles, longest
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

    /// The least speed there is: a faster model's only up to what the host
    /// has.
    fn least(&self, protected: bool, host_max: u32) -> u32 {
        if protected {
            self.base
                .max(PROTECTED_LEAST)
                .max(self.protected_least.min(host_max))
        } else {
            self.base
        }
    }

    /// The speed to go on at from `current`, after a measurement there.
    ///
    /// Below the speed where it stops, a program's frame rate follows the
    /// speed: from the frame rates at two speeds, one below that point, it
    /// is at the speed the lower one would have grown to the higher frame
    /// rate at.
    pub fn decide(&mut self, current: u32, m: &Measurement, protected: bool, host_max: u32) -> u32 {
        let base = self.least(protected, host_max);
        let top = host_max.max(base);
        let c = current as f64;
        self.flat_out = false;
        if protected && m.halted >= IDLES {
            self.idles = true;
        }
        // The host fell behind at this speed: a step less, and no more than
        // that until it has kept up for a while. Behind again right after,
        // down to what it ran (no less than half): a step a measurement
        // from where a step up went too far would leave it behind for
        // many.
        let kept_up = self.pace >= KEPT_UP;
        self.pace = self.pace * 0.75 + m.pace * 0.25;
        let lagged = m.pace < KEPT_UP && self.pace < KEPT_UP;
        let again = std::mem::replace(&mut self.behind, lagged);
        // Behind once at well under what the host runs, after keeping up,
        // it stalled: that says nothing of its limit, and a cap from it
        // would hold back the work of a program that hit one while
        // starting. Behind every other measurement, the smoothed pace stays
        // low, and that is no stall. A cap at the least speed holds nothing
        // but such work back either.
        let stalled = kept_up && self.lagged.is_none() && c * STALLED < top as f64;
        if lagged && !stalled {
            let cut = if again {
                (m.pace * KEPT_UP).clamp(0.5, 1.0 / FINE_STEP)
            } else {
                1.0 / FINE_STEP
            };
            // Of the speed that fell behind: an estimate that dipped for the
            // frames it was measured on would hold the limit down after it.
            let most = (c * cut).round() as u32;
            self.lagged = (most > base).then_some((most, LAGGED_WINDOWS));
        } else if let (false, Some((most, left))) = (lagged, self.lagged) {
            // The estimate dipping to the limit doesn't end it: only rising
            // past what the host has does.
            self.lagged = if left > 1 {
                Some((most, left - 1))
            } else {
                let most = (most as f64 * FINE_STEP) as u32;
                (most < top).then_some((most, LAGGED_WINDOWS))
            };
        }
        // The cap was clamped to the least speed of its own measurement; a
        // host estimate that has risen since raises the least speed of a
        // faster CPU model past it.
        let host_top = top;
        let top = self.lagged.map_or(top, |(most, _)| most.clamp(base, top));
        let clamp = |speed: f64| (speed.round() as u32).clamp(base, top);
        if m.polling >= WAITING {
            self.restart_search();
            return clamp(c * WAITING_STEP);
        }
        let Some(fps) = m.fps else {
            self.restart_search();
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
                self.flat_out = true;
                top
            } else if self.busy_os && self.idles && m.halted < BUSY {
                self.flat_out = true;
                top
            } else {
                clamp(c * WAITING_STEP)
            };
        };
        // Frames nobody sees: as many less as the frame rate is too high
        // (it follows the speed there).
        if fps > m.refresh / AT_REFRESH {
            self.step = Step::Hold {
                fps: m.refresh,
                left: HOLD_WINDOWS,
                more: false,
                recent: m.refresh,
                seen: 0,
                dipped: false,
            };
            return clamp(c * m.refresh / fps);
        }
        // Hold at `speed`; the frame rate to hold is measured there, unless
        // that is here.
        let hold = |this: &mut Self, speed: f64, left: u32, more: bool| {
            let speed = clamp(speed);
            let fps = if speed == current { fps } else { f64::NAN };
            this.step = Step::Hold {
                fps,
                left,
                more,
                recent: fps,
                seen: 0,
                dipped: false,
            };
            speed
        };
        // Whatever the search was doing, it holds there, and measures the
        // frame rate it draws.
        if lagged && current > top {
            return hold(self, top as f64, HOLD_WINDOWS, false);
        }
        // More, by as much as would take the frame rate to the display's.
        let up = |this: &mut Self, most: f64, below: (u32, f64)| {
            if fps >= m.refresh * AT_REFRESH || current >= top {
                return hold(this, c, HOLD_WINDOWS, false);
            }
            this.step = Step::Raised { from: current, fps, below };
            clamp(c * (m.refresh / fps).clamp(FINE_STEP, most))
        };
        let down = |this: &mut Self, fps: f64| {
            if current <= base {
                return hold(this, c, HOLD_WINDOWS, true);
            }
            this.step = Step::Lowered {
                from: current,
                fps,
                sum: 0.0,
                seen: 0,
            };
            clamp(c / FINE_STEP)
        };
        // The step up gained nothing, and `fps` is what the program draws
        // with all it could want: the frame rate to keep, or the reckoning
        // stays under it when more speed still gains a little (Myth, whose
        // frames also wait for the card). Reckon from the speed before the
        // step, and check the reckoning with a small step up once `JUDGED`
        // measurements are in (frame rates that grow in jumps make it too
        // low).
        let overshot = |this: &mut Self, below: (u32, f64), fps: f64| {
            if !lagged && this.held.is_none_or(|r| fps >= r.fps) {
                this.held = Some(Record {
                    fps,
                    at: current.min(top),
                    climbed: 0,
                });
            }
            let (low, low_fps) = (below.0 as f64, below.1);
            let knee = (low * fps / low_fps).clamp(low, c);
            hold(this, knee * HEADROOM, JUDGED, true)
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
                } else if current > top {
                    overshot(self, below, fps)
                } else {
                    // A big step may have gone past the point already. This
                    // measurement is the first after a change of speed, so
                    // it is the mean of two.
                    self.step = Step::Overshot {
                        below,
                        sum: fps * 2.0,
                        seen: 2,
                    };
                    current
                }
            }
            Step::Overshot { below, sum, seen } if current > below.0 => {
                let (sum, seen) = (sum + fps, seen + 1);
                // Not at a speed the host stopped keeping up with, or says
                // it won't: what it measured so far has to do.
                if seen < JUDGED && m.pace >= KEPT_UP && current <= top {
                    self.step = Step::Overshot { below, sum, seen };
                    return current;
                }
                overshot(self, below, sum / seen as f64)
            }
            Step::Lowered {
                from,
                fps: before,
                sum,
                seen,
            } if current < from => {
                let (sum, seen) = (sum + fps, seen + 1);
                // A frame rate that fell by a fifth is no noise to average
                // out: the program shouldn't wait out the rest of them.
                if seen < JUDGED && fps >= before * DROPPED {
                    self.step = Step::Lowered {
                        from,
                        fps: before,
                        sum,
                        seen,
                    };
                    return current;
                }
                let fps = sum / seen as f64;
                let kept = self.held.map_or(before, |r| r.fps.max(before));
                if fps >= kept * KEPT {
                    self.held = self.held.map(|r| Record {
                        at: current,
                        climbed: 0,
                        ..r
                    });
                    down(self, fps)
                } else {
                    let left = HOLD_WINDOWS << self.backoffs;
                    self.backoffs = (self.backoffs + 1).min(MOST_BACKOFFS);
                    if fps >= before * KEPT {
                        // Only the held frame rate stopped it: `from` kept
                        // enough of that, and a knee reckoned from this step
                        // would put the hold above it, a little more each
                        // time.
                        hold(self, from as f64, left, true)
                    } else {
                        let knee = (c * before / fps).clamp(c, from as f64);
                        hold(self, (knee * HEADROOM).min(from as f64 * HEADROOM), left, true)
                    }
                }
            }
            Step::Hold {
                fps: held,
                left,
                more,
                recent: before,
                seen: seen_before,
                dipped,
            } => {
                let held = if held.is_nan() { fps } else { held };
                let recent = if before.is_nan() {
                    fps
                } else {
                    before * 0.75 + fps * 0.25
                };
                let seen = seen_before + 1;
                let low = (recent + fps) / 2.0 < held * DROPPED;
                if low && !dipped {
                    self.step = Step::Hold {
                        fps: held,
                        left,
                        more,
                        recent: before,
                        seen: seen_before,
                        dipped: true,
                    };
                    return current;
                }
                if low {
                    // A heavier scene: what the last one held at says
                    // nothing of it.
                    self.held = None;
                    self.backoffs = 0;
                    if !lagged && self.lagged.is_some() && current >= top && top < host_top {
                        let most = (top as f64 * HEAVIER) as u32;
                        self.lagged = (most < host_top).then_some((most, LAGGED_WINDOWS));
                        self.step = Step::Raised { from: current, fps, below: here };
                        let more = c * (m.refresh / fps).clamp(FINE_STEP, FIRST_STEP);
                        return (more.round() as u32).clamp(base, most.min(host_top));
                    }
                    return up(self, FIRST_STEP, here);
                }
                let record = Record {
                    fps: recent,
                    at: current,
                    climbed: 0,
                };
                let fewer = |r: Record| recent < r.fps * KEPT && seen >= JUDGED;
                match self.held {
                    // Fewer frames than the record's speed drew: back to
                    // it, or further up if it is there, and down from
                    // there as far as that keeps them.
                    Some(r) if fewer(r) && current < r.at.min(top) => {
                        return hold(self, r.at as f64, JUDGED, false);
                    }
                    Some(r) if fewer(r) && r.climbed < CLIMBS && current < top => {
                        let at = clamp(c * FINE_STEP);
                        self.held = Some(Record {
                            at,
                            climbed: r.climbed + 1,
                            ..r
                        });
                        return hold(self, at as f64, JUDGED, false);
                    }
                    // One measurement is no record, as for steps down.
                    _ if lagged || seen < JUDGED => {}
                    None => self.held = Some(record),
                    Some(r) if fewer(r) && current >= r.at.min(host_top) => {
                        self.held = Some(record);
                    }
                    Some(r) if recent * KEPT > r.fps => self.held = Some(record),
                    Some(_) => {}
                }
                if left > 1 {
                    self.step = Step::Hold {
                        fps: held,
                        left: left - 1,
                        more,
                        recent,
                        seen,
                        dipped: false,
                    };
                    current
                } else if more {
                    up(self, FINE_STEP, here)
                } else {
                    down(self, recent)
                }
            }
            // The speed changed from elsewhere: measure from here.
            Step::Raised { .. } | Step::Overshot { .. } | Step::Lowered { .. } => {
                up(self, FIRST_STEP, here)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: u32 = 800_000;

    /// A measurement without frames, of a program that keeps real time.
    const STILL: Measurement = Measurement {
        fps: None,
        polling: 0.0,
        halted: 0.0,
        output: 0.0,
        refresh: 70.0,
        pace: 1.0,
    };

    /// A game that draws `per_cycle` frames a second for each instruction
    /// per ms, up to `cap` frames a second.
    fn game(per_cycle: f64, cap: f64) -> impl Fn(u32) -> Measurement {
        move |speed| Measurement {
            fps: Some((speed as f64 * per_cycle).min(cap)),
            polling: 0.0,
            halted: 0.0,
            output: 0.0,
            refresh: 70.0,
            pace: 1.0,
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
        let speeds = run(game(35.0 / 60_000.0, 35.0), true, 60);
        assert!((60_000..=85_000).contains(&speeds[12]), "{:?}", speeds);
        // A while later it tries less, finds that loses frames, and goes
        // back.
        assert!(speeds[12..].iter().all(|&s| s >= 45_000), "{:?}", speeds);
        assert!(speeds[59] >= 60_000, "{:?}", speeds);
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
            pace: 1.0,
        };
        let speeds = run(f117, false, 60);
        assert!((20_000..=30_000).contains(&speeds[59]), "{:?}", speeds);
    }

    /// Myth's 3Dfx renderer, as measured: its frame rate at speeds from
    /// 9000 to the game's cap of 30 a second. Above 40,000 each step down
    /// loses 1-3% of the frames.
    fn myth(speed: u32) -> Measurement {
        const CURVE: [(f64, f64); 7] = [
            (9_000.0, 3.0),
            (36_000.0, 20.6),
            (44_000.0, 26.0),
            (56_000.0, 27.7),
            (75_000.0, 28.8),
            (110_000.0, 29.8),
            (130_000.0, 30.0),
        ];
        let c = speed as f64;
        let above = CURVE
            .iter()
            .position(|p| p.0 >= c)
            .unwrap_or(CURVE.len() - 1)
            .max(1);
        let ((x0, y0), (x1, y1)) = (CURVE[above - 1], CURVE[above]);
        let fps = (y0 + (y1 - y0) * (c - x0) / (x1 - x0)).clamp(1.0, 30.0);
        Measurement {
            fps: Some(fps),
            refresh: 60.0,
            ..STILL
        }
    }

    #[test]
    fn steps_down_lose_no_more_than_kept_of_the_held_frame_rate() {
        let speeds = run(myth, true, 300);
        let fps = |speed| myth(speed).fps.unwrap();
        // Steps down stop within 3% of the 30 frames a second and go back
        // up; the one that went too far is judged and left.
        let later = &speeds[100..];
        assert!(later.iter().all(|&s| s >= 60_000), "{:?}", speeds);
        let fewer = later.iter().filter(|&&s| fps(s) < 28.7).count();
        assert!(fewer <= later.len() / 5, "{:?}", speeds);
        // And it holds where the last step that kept enough went, without
        // creeping up a little after each step that didn't.
        let last = later[later.len() - 1];
        let most = later.iter().filter(|&&s| s == last).count();
        assert!(most >= later.len() * 4 / 5, "{:?}", speeds);
    }

    #[test]
    fn a_frame_rate_held_faster_than_the_host_goes_now_does_not_stop_steps_down() {
        // Myth again, with the host slowed to 100,000 after a while: the
        // record's speed can't be reached again, and what the hold at
        // 100,000 draws is the frame rate to keep.
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 3000;
        let mut speeds = Vec::new();
        for i in 0..400 {
            let host = if i < 150 { HOST } else { 100_000 };
            speed = auto.decide(speed, &myth(speed), true, host);
            speeds.push(speed);
        }
        let below = speeds[300..].iter().filter(|&&s| s < 100_000).count();
        assert!(below >= 100 / 3, "{:?}", speeds);
    }

    /// The speeds `auto` runs Myth at, one a measurement, on a host that
    /// keeps real time up to `keeps_up(i)` instructions per ms at the
    /// `i`th measurement and falls behind in proportion above it, while
    /// its estimate says it has `HOST`.
    fn run_myth_behind(keeps_up: impl Fn(usize) -> u32, measurements: usize) -> Vec<u32> {
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 3000;
        (0..measurements)
            .map(|i| {
                let pace = (keeps_up(i) as f64 / speed as f64).min(1.0);
                speed = auto.decide(
                    speed,
                    &Measurement {
                        pace,
                        ..myth(speed)
                    },
                    true,
                    HOST,
                );
                speed
            })
            .collect()
    }

    #[test]
    fn a_speed_the_host_does_not_keep_up_with_is_left() {
        // Myth's battle: for 150 measurements the host keeps real time up
        // to 40,000 only, below the 50,000 the speed held at before and
        // the frame rate held there.
        let battle = 150..300;
        let speeds = run_myth_behind(
            |i| {
                if battle.contains(&i) {
                    40_000
                } else {
                    u32::MAX
                }
            },
            450,
        );
        assert!(speeds[battle.start - 1] >= 45_000, "{:?}", speeds);
        // Measured behind (`speeds[i]` is the speed of measurement i + 1):
        // the first, and a few when a step up tries more again. The host
        // estimate here stays far above the host, so each takes a second
        // measurement to tell it from a stall.
        let behind = |s: &&u32| **s as f64 * 0.95 > 40_000.0;
        let lost = speeds[battle.start - 1..battle.end - 1]
            .iter()
            .filter(behind)
            .count();
        assert!(lost <= battle.len() / 7, "{} behind: {:?}", lost, speeds);
        // Without sliding to the floor, and back up after the battle.
        assert!(
            speeds[battle.start..].iter().all(|&s| s >= 25_000),
            "{:?}",
            speeds
        );
        assert!(speeds[400..].iter().all(|&s| s >= 40_000), "{:?}", speeds);
    }

    #[test]
    fn a_host_that_stalls_for_a_moment_costs_a_step_not_the_slide() {
        // The whole host stalls during a measurement: emulated time runs
        // at about a fifth of real time there, whatever the speed.
        let speeds = run_myth_behind(|i| if i == 200 { 10_000 } else { u32::MAX }, 450);
        assert!(speeds[200..].iter().all(|&s| s >= 30_000), "{:?}", speeds);
        assert!(speeds[400..].iter().all(|&s| s >= 40_000), "{:?}", speeds);
    }

    /// Noise of up to `share` of the frame rate, as Myth's measurements have.
    fn noise(seed: &mut u32, share: f64) -> f64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 17;
        *seed ^= *seed << 5;
        1.0 + share * (*seed as f64 / u32::MAX as f64 * 2.0 - 1.0)
    }

    #[test]
    fn noise_does_not_walk_the_speed_down_or_send_it_far_up() {
        // Over many seeds: noise that reads as a gain now and then sends
        // one run up for a while, and it comes back.
        let (mut slow, mut high, mut all) = (0, 0, 0);
        for run in 1..41u32 {
            let mut auto = AutoSpeed::new(3000, None);
            let (mut speed, mut seed) = (3000, run.wrapping_mul(0x9E37_79B9) | 1);
            let mut speeds = Vec::new();
            for _ in 0..3000 {
                let m = myth(speed);
                let fps = m.fps.map(|fps| fps * noise(&mut seed, 0.06));
                speed = auto.decide(speed, &Measurement { fps, ..m }, true, HOST);
                speeds.push(speed);
            }
            let later = &speeds[300..];
            slow += later.iter().filter(|&&s| s < 50_000).count();
            high += later.iter().filter(|&&s| s > 300_000).count();
            all += later.len();
            let mean = later.iter().map(|&s| s as f64).sum::<f64>() / later.len() as f64;
            assert!((60_000.0..=200_000.0).contains(&mean), "{}", mean);
        }
        // A tenth of the time is about what the game's own runs spend there.
        assert!(slow <= all / 10, "{} of {} slow", slow, all);
        assert!(high <= all / 200, "{} of {} high", high, all);
    }

    #[test]
    fn deep_stalls_now_and_then_do_not_compound() {
        // The host loses half of a measurement every so often: each costs
        // a step, which comes back before the next.
        for every in [40, 80, 200] {
            let mut auto = AutoSpeed::new(3000, None);
            let mut speed = 3000;
            let mut speeds = Vec::new();
            for i in 0..2000 {
                let pace = if i > 200 && i % every == 0 { 0.5 } else { 1.0 };
                let m = Measurement {
                    pace,
                    ..myth(speed)
                };
                speed = auto.decide(speed, &m, true, HOST);
                speeds.push(speed);
            }
            let later = &speeds[400..];
            assert!(later.iter().all(|&s| s >= 44_000), "{}: {:?}", every, later);
            let mean = later.iter().map(|&s| s as f64).sum::<f64>() / later.len() as f64;
            assert!(mean >= 70_000.0, "{}: {}", every, mean);
        }
    }

    #[test]
    fn a_stall_while_starting_does_not_hold_back_the_work_after_it() {
        // Unreal Gold's first measurement, at the real-mode speed with a
        // host estimate not settled yet, and a stall at the speed of a
        // menu: neither says what the host runs when the loading starts.
        for (speed, host) in [(3000, 4000), (40_000, 450_000)] {
            let mut auto = AutoSpeed::new(3000, None);
            let stalled = Measurement {
                pace: 0.5,
                output: 1_000_000.0,
                ..STILL
            };
            auto.decide(speed, &stalled, true, host);
            let work = Measurement {
                output: 1_000_000.0,
                ..STILL
            };
            assert_eq!(auto.decide(20_000, &work, true, 450_000), 450_000);
        }
    }

    #[test]
    fn a_program_without_frames_stays_near_what_the_host_keeps_up_with() {
        // Magic & Mayhem's battles: the host runs 140,000, and light frames
        // now and then make its estimate several times that.
        let mut auto = AutoSpeed::new(3000, None);
        let (mut speed, mut seed) = (200_000, 0x1234_5678);
        let mut paces = Vec::new();
        for i in 0..400 {
            let pace = (140_000.0 / speed as f64).min(1.0);
            let m = Measurement {
                pace,
                output: 50_000.0,
                ..STILL
            };
            let estimate = 200_000.0
                * if i % 7 == 0 {
                    2.5
                } else {
                    noise(&mut seed, 0.2)
                };
            speed = auto.decide(speed, &m, true, estimate as u32);
            paces.push(pace);
        }
        let later = &paces[100..];
        let mean = later.iter().sum::<f64>() / later.len() as f64;
        assert!(mean >= 0.9, "{}: {:?}", mean, paces);
        assert!(later.iter().all(|&pace| pace >= 0.5), "{:?}", paces);
    }

    #[test]
    fn hitches_of_the_host_do_not_take_the_speed_to_the_floor() {
        // One backlog dropped in a measurement every 20 seconds or so,
        // whatever the speed: the host otherwise keeps up.
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 3000;
        let mut speeds = Vec::new();
        for i in 0..1000 {
            let pace = if i > 200 && i % 40 == 0 { 0.93 } else { 1.0 };
            let m = Measurement {
                pace,
                ..myth(speed)
            };
            speed = auto.decide(speed, &m, true, HOST);
            speeds.push(speed);
        }
        assert!(speeds[200..].iter().all(|&s| s >= 60_000), "{:?}", speeds);
    }

    #[test]
    fn a_host_estimate_rising_past_the_lagged_cap_raises_it() {
        // A Pentium II's least speed, 112500, is the host's estimate while
        // that is less. Behind at 60000, the cap is that least speed; the
        // estimate rises to 70000 and the least speed with it, above the
        // cap.
        let mut auto = AutoSpeed::new(3000, None);
        auto.set_protected_least(112_500);
        let behind = Measurement {
            pace: 0.5,
            ..myth(60_000)
        };
        assert_eq!(auto.decide(60_000, &behind, true, 60_000), 60_000);
        // The hold goes on at 60000, then goes to the least speed.
        let mut speed = 60_000;
        for _ in 0..2 * HOLD_WINDOWS {
            speed = auto.decide(speed, &myth(speed), true, 70_000);
        }
        assert_eq!(speed, 70_000);
    }

    #[test]
    fn a_lighter_scene_held_at_does_not_stop_later_steps_down() {
        // Heretic, which draws half as many frames again for a few
        // seconds while it holds (looking at the sky), up to 50.
        let heretic = game(35.0 / 60_000.0, 35.0);
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 3000;
        let mut speeds = Vec::new();
        for i in 0..80 {
            let mut m = heretic(speed);
            if (15..20).contains(&i) {
                m.fps = m.fps.map(|fps| (fps * 1.5).min(50.0));
            }
            speed = auto.decide(speed, &m, true, HOST);
            speeds.push(speed);
        }
        // The lighter scene comes while the speed holds, so it is held at.
        assert!(
            speeds[14..20].iter().all(|&s| s == speeds[14]),
            "{:?}",
            speeds
        );
        // Back to 35 at the same speed, the 50 says nothing of what a step
        // down loses: it tries one against the 35.
        assert!(speeds[20..].iter().any(|&s| s < speeds[14]), "{:?}", speeds);
    }

    #[test]
    fn a_slightly_heavier_scene_goes_back_to_the_held_speed() {
        // Myth: a tenth fewer frames, too few to be a heavier scene by
        // `DROPPED`, while the speed holds below the one that drew 30.
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 3000;
        for _ in 0..100 {
            speed = auto.decide(speed, &myth(speed), true, HOST);
        }
        let held = speed * 2;
        auto.held = Some(Record {
            fps: 30.0,
            at: held,
            climbed: 0,
        });
        let mut most = 0;
        for _ in 0..12 {
            let m = myth(speed);
            let heavier = Measurement {
                fps: m.fps.map(|fps| fps * 0.9),
                ..m
            };
            speed = auto.decide(speed, &heavier, true, HOST);
            most = most.max(speed);
        }
        assert!((held..=held * 4 / 3).contains(&most), "{}", most);
    }

    #[test]
    fn a_slightly_heavier_scene_does_not_lower_what_steps_down_keep_to() {
        // Steps down took the record's speed to 60,000, and the scene now
        // draws 26 there: more speed may bring the 30 back, so it climbs
        // to find out, and the record stays until it has.
        let mut auto = AutoSpeed::new(3000, None);
        let (mut speed, mut speeds) = (60_000, Vec::new());
        auto.held = Some(Record {
            fps: 30.0,
            at: speed,
            climbed: 0,
        });
        auto.step = Step::Hold {
            fps: 26.0,
            left: HOLD_WINDOWS,
            more: false,
            recent: 26.0,
            seen: 0,
            dipped: false,
        };
        for _ in 0..JUDGED * 2 + 1 {
            // More speed gains the scene its frames back from 75,000 on.
            let fps = if speed >= 75_000 { 30.0 } else { 26.0 };
            let m = Measurement {
                fps: Some(fps),
                ..STILL
            };
            speed = auto.decide(speed, &m, true, HOST);
            speeds.push(speed);
            assert_eq!(auto.held.map(|r| r.fps), Some(30.0), "{:?}", speeds);
        }
        assert!(speeds.iter().any(|&s| s >= 75_000), "{:?}", speeds);
        // Where there is no such speed, the record comes down above where
        // it was missed.
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 60_000;
        auto.held = Some(Record {
            fps: 30.0,
            at: speed,
            climbed: 0,
        });
        auto.step = Step::Hold {
            fps: 26.0,
            left: HOLD_WINDOWS,
            more: false,
            recent: 26.0,
            seen: 0,
            dipped: false,
        };
        while auto.held.is_some_and(|r| r.fps == 30.0) {
            let m = Measurement {
                fps: Some(26.0),
                ..STILL
            };
            speed = auto.decide(speed, &m, true, HOST);
        }
        assert!(
            auto.held.is_some_and(|r| r.fps == 26.0 && r.at > 70_000),
            "{:?}",
            auto.held
        );
    }

    /// Doom's `i`th measurement at `speed`: frames follow the speed up to
    /// the game's cap of 35 a second, from about 60,000, and a cap measured
    /// over half a second reads a frame high or low.
    fn doom(speed: u32, i: usize) -> Measurement {
        let reading: f64 = [35.3, 35.3, 34.3, 35.3, 34.8][i % 5];
        Measurement {
            fps: Some(reading.min(speed as f64 * 35.0 / 60_000.0)),
            ..STILL
        }
    }

    /// `auto` with Doom settled, holding and `measurements` in.
    fn doom_settled(measurements: usize) -> (AutoSpeed, u32) {
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 3000;
        for i in 0..measurements {
            speed = auto.decide(speed, &doom(speed, i), false, HOST);
        }
        (auto, speed)
    }

    #[test]
    fn a_screen_wipe_leaves_the_speed_and_the_record() {
        let (mut auto, mut speed) = doom_settled(300);
        assert!((60_000..=95_000).contains(&speed), "{}", speed);
        let (held, step) = (auto.held, auto.step);
        let wipe = Measurement {
            fps: Some(21.8),
            ..STILL
        };
        assert_eq!(auto.decide(speed, &wipe, false, HOST), speed);
        assert_eq!(auto.held, held);
        // The next measurement goes on from where the search was.
        speed = auto.decide(speed, &doom(speed, 0), false, HOST);
        assert!(speed <= 95_000, "{}", speed);
        match (auto.step, step) {
            (Step::Hold { seen, .. }, Step::Hold { seen: before, .. }) => {
                assert_eq!(seen, before + 1)
            }
            other => panic!("{:?}", other),
        }
    }

    #[test]
    fn a_lucky_measurement_at_an_overshoot_sets_no_record() {
        // A wipe across two measurements sends the speed up from 80,000,
        // and the first measurement up there reads 36.36. At that record
        // steps down that lost nothing read as a loss, and the speed walks
        // down over minutes from above where it went.
        let (mut auto, mut speed) = doom_settled(300);
        let wipe = Measurement {
            fps: Some(21.8),
            ..STILL
        };
        speed = auto.decide(speed, &wipe, false, HOST);
        speed = auto.decide(speed, &wipe, false, HOST);
        let top = speed;
        assert!(top > 200_000, "{}", top);
        let lucky = Measurement {
            fps: Some(36.36),
            ..STILL
        };
        speed = auto.decide(speed, &lucky, false, HOST);
        let mut speeds = vec![speed];
        for i in 0..120 {
            speed = auto.decide(speed, &doom(speed, i), false, HOST);
            speeds.push(speed);
        }
        let record = auto.held.map_or(0.0, |r| r.fps);
        assert!(record * KEPT < 34.9, "{}: {:?}", record, speeds);
        assert!(speeds.iter().all(|&s| s <= top), "{:?}", speeds);
        assert!(speeds[100..].iter().all(|&s| s <= 95_000), "{:?}", speeds);
    }

    /// Past the host's estimate, as Myth's step to 142,000 is: 28.7 frames
    /// a second at 68,000, and 30 at the step.
    fn past_the_estimate(host: u32) -> (AutoSpeed, u32) {
        let mut auto = AutoSpeed::new(3000, None);
        auto.step = Step::Raised {
            from: 68_000,
            fps: 28.7,
            below: (34_000, 28.7),
        };
        let speed = auto.decide(142_000, &myth_at(30.0), true, host);
        (auto, speed)
    }

    fn myth_at(fps: f64) -> Measurement {
        Measurement {
            fps: Some(fps),
            ..STILL
        }
    }

    #[test]
    fn a_step_past_the_host_estimate_does_not_stay() {
        let (auto, speed) = past_the_estimate(100_000);
        assert!(matches!(auto.step, Step::Hold { .. }), "{:?}", auto.step);
        assert!(speed <= 100_000, "{}", speed);
    }

    #[test]
    fn the_host_estimate_falling_ends_a_stay() {
        let mut auto = AutoSpeed::new(3000, None);
        auto.step = Step::Overshot {
            below: (34_000, 28.7),
            sum: 60.0,
            seen: 2,
        };
        let speed = auto.decide(142_000, &myth_at(30.0), true, 100_000);
        assert!(matches!(auto.step, Step::Hold { .. }), "{:?}", auto.step);
        assert!(speed <= 100_000, "{}", speed);
    }

    #[test]
    fn a_record_past_the_host_estimate_comes_back_to_the_estimate() {
        // Going back to 142,000 for the record's frames would run the sound
        // dry again, each time the frame rate dips under it.
        let (auto, _) = past_the_estimate(120_000);
        let record = auto.held.expect("a record");
        assert_eq!(record.at, 120_000);
        assert_eq!(record.fps, 30.0);
    }

    #[test]
    fn a_step_within_the_host_estimate_stays_until_judged() {
        let (mut auto, speed) = past_the_estimate(800_000);
        assert_eq!(speed, 142_000);
        assert!(matches!(auto.step, Step::Overshot { seen: 2, .. }));
        assert_eq!(auto.decide(speed, &myth_at(30.0), true, 800_000), speed);
        assert!(auto.decide(speed, &myth_at(30.0), true, 800_000) < speed);
        assert!(matches!(auto.step, Step::Hold { .. }), "{:?}", auto.step);
        assert_eq!(auto.held.expect("a record").at, speed);
    }

    #[test]
    fn a_speed_cut_below_the_step_during_a_stay_starts_from_there() {
        // The reckoning takes its range from the step: a host that took the
        // speed under where the step began leaves it none.
        let mut auto = AutoSpeed::new(3000, None);
        auto.step = Step::Overshot {
            below: (68_000, 28.7),
            sum: 60.0,
            seen: 2,
        };
        auto.decide(30_000, &myth_at(28.0), true, 30_000);
        assert!(!matches!(auto.step, Step::Overshot { .. }), "{:?}", auto.step);
    }

    #[test]
    fn a_hold_sets_no_record_from_its_first_measurements() {
        for held in [None, Some(35.0)] {
            let mut auto = AutoSpeed::new(3000, None);
            auto.held = held.map(|fps| Record {
                fps,
                at: 80_000,
                climbed: 0,
            });
            auto.step = Step::Hold {
                fps: f64::NAN,
                left: HOLD_WINDOWS,
                more: false,
                recent: f64::NAN,
                seen: 0,
                dipped: false,
            };
            let lucky = Measurement {
                fps: Some(36.36),
                ..STILL
            };
            auto.decide(80_000, &lucky, false, HOST);
            assert_eq!(auto.held.map(|r| r.fps), held);
            // Enough of them are a frame rate.
            for _ in 1..JUDGED {
                auto.decide(80_000, &lucky, false, HOST);
            }
            assert!(auto.held.is_some_and(|r| r.fps > 36.0));
        }
    }

    #[test]
    fn a_record_at_an_overshoot_is_what_the_speed_draws_on_average() {
        // Myth's record comes from the step that went too far, as it must
        // for steps down to stop at 97% of the 30 it draws up there, and
        // the mean of its noise is as good as one measurement of it.
        let mut auto = AutoSpeed::new(3000, None);
        let (mut speed, mut seed) = (3000, 0x1234_5678);
        while !matches!(auto.step, Step::Overshot { .. }) {
            let m = myth(speed);
            let fps = m.fps.map(|fps| fps * noise(&mut seed, 0.06));
            speed = auto.decide(speed, &Measurement { fps, ..m }, true, HOST);
        }
        let overshoot = speed;
        for reading in [31.0, 28.0] {
            let m = Measurement {
                fps: Some(reading),
                refresh: 60.0,
                ..STILL
            };
            assert!(auto.held.is_none());
            speed = auto.decide(speed, &m, true, HOST);
        }
        let record = auto.held.unwrap();
        assert!((29.0..31.0).contains(&record.fps), "{:?}", record);
        assert_eq!(record.at, overshoot);
        assert!(speed < overshoot);
    }

    #[test]
    fn an_overshoot_the_host_does_not_keep_up_with_is_left_at_once() {
        // Myth: the record is still what was measured up there, and the
        // speed doesn't stay for the rest of the measurements.
        let mut auto = AutoSpeed::new(3000, None);
        auto.step = Step::Overshot {
            below: (68_000, 28.7),
            sum: 60.0,
            seen: 2,
        };
        let m = Measurement {
            pace: 0.93,
            ..myth(142_000)
        };
        assert!(auto.decide(142_000, &m, true, HOST) < 100_000);
        assert_eq!(auto.held.map(|r| r.at), Some(142_000));
    }

    #[test]
    fn steps_down_that_lose_frames_are_tried_less_and_less_often() {
        // Doom in a scene whose knee is at 40,000: where it comes from is
        // right, and a step below that loses frames each time.
        let scene = |speed: u32| Measurement {
            fps: Some((speed as f64 * 35.0 / 40_000.0).min(35.0)),
            ..STILL
        };
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 3000;
        let mut tries = Vec::new();
        for i in 0..1000 {
            speed = auto.decide(speed, &scene(speed), false, HOST);
            if matches!(auto.step, Step::Lowered { seen: 0, .. }) && speed < 40_000 {
                tries.push(i);
            }
        }
        assert!(tries.len() >= 4, "{:?}", tries);
        let gaps: Vec<usize> = tries.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(gaps[0] >= HOLD_WINDOWS as usize, "{:?}", gaps);
        assert!(*gaps.last().unwrap() >= 4 * HOLD_WINDOWS as usize, "{:?}", gaps);
        // A heavier scene starts over.
        assert_eq!(auto.backoffs, MOST_BACKOFFS);
        for _ in 0..2 {
            let m = Measurement {
                fps: Some(10.0),
                ..STILL
            };
            speed = auto.decide(40_000, &m, false, HOST);
        }
        assert_eq!(auto.backoffs, 0, "{}", speed);
    }

    #[test]
    fn a_step_down_that_loses_a_fifth_of_the_frames_is_left_at_once() {
        let mut auto = AutoSpeed::new(3000, None);
        auto.step = Step::Lowered {
            from: 100_000,
            fps: 30.0,
            sum: 0.0,
            seen: 0,
        };
        let m = Measurement {
            fps: Some(20.0),
            refresh: 60.0,
            ..STILL
        };
        assert!(auto.decide(87_000, &m, true, HOST) > 87_000);
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
        let nothing = |halted, output| {
            move |_| Measurement {
                halted,
                output,
                ..STILL
            }
        };
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
        let loading = Measurement {
            output: 1e6,
            ..STILL
        };
        assert_eq!(auto.decide(PROTECTED_LEAST, &loading, true, HOST), HOST);
        assert_eq!(auto.working, Some(PROTECTED_LEAST), "where it goes back to");
        // Anything else isn't work.
        let spinning = Measurement { output: 0.0, ..loading };
        auto.working = None;
        auto.decide(HOST, &spinning, true, HOST);
        assert_eq!(auto.working, None);
    }

    #[test]
    fn a_program_given_all_the_host_has_follows_it_down_at_once() {
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        let bus = &mut cpu.bus;
        bus.set_cycles_per_ms(HOST);
        let mut auto = AutoSpeed::new(3000, None);
        let loading = Measurement {
            output: 1e6,
            ..STILL
        };
        assert_eq!(auto.decide(PROTECTED_LEAST, &loading, true, HOST), HOST);
        // The next frame, no measurement yet.
        assert_eq!(auto.update(bus, true, HOST / 2, 0), Some(HOST / 2));
        assert_eq!(
            auto.update(bus, true, HOST, 0),
            None,
            "up takes a measurement"
        );
        // Never below the least speed.
        assert_eq!(auto.update(bus, true, 1000, 0), Some(PROTECTED_LEAST));
        // A speed that isn't all the host has stays until a measurement.
        auto.reset();
        assert_eq!(auto.update(bus, true, HOST / 2, 0), None);
    }

    #[test]
    fn work_done_goes_back_no_higher_than_the_host_keeps_up_with() {
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        let bus = &mut cpu.bus;
        bus.set_cycles_per_ms(HOST);
        let mut auto = AutoSpeed::new(3000, None);
        auto.working = Some(200_000);
        // Behind while it worked.
        auto.lagged = Some((150_000, LAGGED_WINDOWS));
        bus.clock.icount += HOST as u64 * 200;
        assert_eq!(auto.update(bus, true, HOST, 0), Some(150_000));
        assert_eq!(auto.lagged, Some((150_000, LAGGED_WINDOWS)));
    }

    #[test]
    fn the_pace_of_measurements_taken_together_covers_their_time() {
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        let bus = &mut cpu.bus;
        bus.set_cycles_per_ms(100_000);
        let mut auto = AutoSpeed::new(3000, None);
        auto.step = Step::Hold {
            fps: 60.0,
            left: HOLD_WINDOWS,
            more: true,
            recent: 60.0,
            seen: 0,
            dipped: false,
        };
        auto.unsettled = Some((f64::NAN, 0));
        assert_eq!(auto.update(bus, true, HOST, 0), None);
        // Half a second each, dropping 25 ms each: the first is the one
        // after a change of speed, which doesn't count yet.
        for (i, lost_ns) in [25_000_000, 50_000_000].into_iter().enumerate() {
            bus.clock.icount += 50_000_000;
            bus.activity.swapped(30);
            assert_eq!(auto.update(bus, true, HOST, lost_ns), None, "{}", i);
        }
        assert_eq!(auto.unsettled, None);
        let pace = 1.0 / 1.05;
        assert!(
            (auto.pace - (0.75 + 0.25 * pace)).abs() < 1e-3,
            "{}",
            auto.pace
        );
    }

    #[test]
    fn a_host_behind_every_other_measurement_is_not_stalling() {
        // Well under the host's estimate, and behind half the time: each
        // one comes right after a measurement that kept up.
        let mut auto = AutoSpeed::new(3000, None);
        let mut speed = 40_000;
        for i in 0..8 {
            let m = Measurement {
                pace: if i % 2 == 0 { 0.5 } else { 1.0 },
                ..myth(speed)
            };
            speed = auto.decide(speed, &m, true, HOST);
        }
        assert!(auto.lagged.is_some(), "{:?}", auto.lagged);
    }

    #[test]
    fn a_host_estimate_dipping_to_the_cap_does_not_end_it() {
        // Magic & Mayhem's light frames push the estimate far above the
        // host, and a stall sinks it for a moment.
        let mut auto = AutoSpeed::new(3000, None);
        auto.lagged = Some((140_000, LAGGED_WINDOWS));
        let work = Measurement {
            output: 1e6,
            ..STILL
        };
        assert_eq!(auto.decide(140_000, &work, true, 130_000), 130_000);
        assert_eq!(auto.decide(130_000, &work, true, 500_000), 140_000);
    }

    #[test]
    fn a_limit_comes_from_the_speed_not_an_estimate_that_dipped() {
        // Unreal's 60 frame a second scene takes the estimate down with it.
        let mut auto = AutoSpeed::new(3000, None);
        auto.pace = 0.9;
        let behind = Measurement {
            pace: 0.8,
            ..game(0.0005, 60.0)(114_817)
        };
        assert_eq!(auto.decide(114_817, &behind, true, 78_135), 78_135);
        assert_eq!(auto.lagged, Some((99_841, LAGGED_WINDOWS)));
    }

    #[test]
    fn a_heavier_scene_lifts_a_limit_a_lighter_one_found() {
        let mut auto = AutoSpeed::new(3000, None);
        auto.lagged = Some((80_000, LAGGED_WINDOWS));
        auto.step = Step::Hold {
            fps: 45.0,
            left: HOLD_WINDOWS,
            more: false,
            recent: 45.0,
            seen: JUDGED,
            dipped: false,
        };
        let scene = |fps| Measurement {
            fps: Some(fps),
            refresh: 60.0,
            ..STILL
        };
        assert_eq!(auto.decide(80_000, &scene(45.0), true, HOST), 80_000);
        // Doubled once it isn't a dip, and no further until the host shows
        // it can run that.
        assert_eq!(auto.decide(80_000, &scene(10.0), true, HOST), 80_000);
        assert_eq!(auto.decide(80_000, &scene(10.0), true, HOST), 160_000);
        assert_eq!(auto.lagged, Some((160_000, LAGGED_WINDOWS)));
    }

    #[test]
    fn a_waiting_program_gets_less() {
        let mut auto = AutoSpeed::new(3000, None);
        let waiting = Measurement {
            polling: 0.5,
            ..STILL
        };
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
        let measure = |halted| Measurement {
            halted,
            refresh: 60.0,
            ..STILL
        };
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
        let waiting = Measurement {
            polling: 0.5,
            ..STILL
        };
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
        bus.activity.set_screen(screen_bytes(bus), linear_screen(bus));
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
                for x in (160..176).step_by(4) {
                    bus.write_32(0xA0000 + (100 + row) * 320 + x, 0x0202_0202);
                }
            }
        }
        bus.clock.icount += 100_000;
        bus.write_8(0xA0000, 1);
        assert_eq!((bus.activity.bursts, bus.activity.copies), (0, 5));
        // Nor is a mouse pointer at the top and a clock at the bottom.
        for _ in 0..5 {
            bus.clock.icount += 100_000;
            for row in 0..16 {
                bus.write_32(0xA0000 + row * 320 + 100, 0x0303_0303);
                bus.write_32(0xA0000 + (190 + row % 8) * 320 + 280, 0x0303_0303);
            }
        }
        bus.clock.icount += 100_000;
        bus.write_8(0xA0000, 1);
        assert_eq!((bus.activity.bursts, bus.activity.copies), (0, 5));
    }

    #[test]
    fn interleaved_rows_make_no_copies() {
        // CGA graphics: odd rows 8 KB above the even ones, so two rows of
        // a sprite reach across half the screen's 16,000 bytes.
        let mut cpu = crate::cpu::Cpu::new(std::path::PathBuf::from("."));
        crate::interrupts::int10::set_mode(&mut cpu, 0x04);
        let bus = &mut cpu.bus;
        bus.activity.set_screen(screen_bytes(bus), linear_screen(bus));
        for _ in 0..5 {
            bus.clock.icount += 100_000;
            for row in 0..200 {
                bus.write_8(0xB8000 + (row % 2) * 0x2000 + (row / 2) * 80 + 40, 0xFF);
            }
        }
        bus.clock.icount += 100_000;
        bus.write_8(0xB8000, 1);
        assert_eq!((bus.activity.bursts, bus.activity.copies), (0, 0));
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
            // Keeping real time: nothing dropped.
            if let Some(speed) = auto.update(bus, protected, 2_000_000, 0) {
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
